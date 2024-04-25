using System.Diagnostics;
using ResourcesWorld;
using ResourcesWorld.wit.imports;

namespace ResourcesWorld.wit.exports
{
    public class ExportsImpl : IExports
    {
        public static IExports.Z Add(IExports.Z a, IExports.Z b)
        {
            var myA = (Z) a;
            var myB = (Z) b;
            return new Z(myA.val + myB.val);
        }
        
        public static Result<None, string> TestImports()
        {
            var y = new IImports.Y(10);
            Debug.Assert(y.GetA() == 10);

            // TODO: test more stuff

            return Result<None, string>.ok(new None());
        }

        public class X : IExports.X, IExports.IX {
            public int val;

            public X(int val) {
                this.val = val;
            }

            public void SetA(int val) {
                this.val = val;
            }

            public int GetA() {
                return val;
            }

            public static IExports.X Add(IExports.X a, int b) {
                var myA = (X) a;
                myA.SetA(myA.GetA() + b);
                return myA;
            }
        }
    
        public class Z : IExports.Z, IExports.IZ {
            private static uint numDropped = 0;
            
            public int val;

            public Z(int val) {
                this.val = val;
            }

            public int GetA() {
                return val;
            }

            public static uint NumDropped() {
                return numDropped + 1;
            }

            override protected void Dispose(bool disposing) {
		numDropped += 1;
                
                base.Dispose(disposing);
            }
        }

        public class KebabCase : IExports.KebabCase, IExports.IKebabCase {
            public uint val;
            
            public KebabCase(uint val) {
                this.val = val;
            }
            
            public uint GetA() {
                return val;
            }

            public static uint TakeOwned(IExports.KebabCase a) {
                var myA = (KebabCase) RepTable.Get((int) a.Handle);
                return myA.val;
            }
        }
    }
}
